import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, readdir, readFile, rm } from 'node:fs/promises';
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
const refusal = "slicc: refused: cookies on seven's origin\n";
const currentValues =
  'Cookie values are from the current cookie jar, not necessarily from this request.\n';
const secrets = [
  'fixture-seven-secret',
  'fixture-seven-sid',
  'fixture-vault-secret',
  'fixture-chip-secret',
  'fixture-seven-rotated',
];

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
  cookie('sid', 'fixture-seven-sid', 'seven.sliccy.ai', { httpOnly: true, sameSite: 'Lax' }),
  cookie('sid', 'fixture-sid-1', 'shop.example.test', { httpOnly: true, sameSite: 'Strict' }),
  cookie('theme', 'fixture-dark', '.example.test', {
    path: '/app',
    expires: 1924992000,
    session: false,
    secure: false,
  }),
  cookie('seven', 'fixture-seven-secret', '.sliccy.ai'),
  cookie('vault', 'fixture-vault-secret', 'vault.sliccy.ai'),
  cookie('chip', 'fixture-chip-secret', 'widget.example.test', {
    partitionKey: { topLevelSite: 'https://sliccy.ai', hasCrossSiteAncestor: true },
  }),
];

const traffic = [
  {
    url: 'https://shop.example.test/api/me',
    requestHeaders: { Accept: '*/*', Cookie: 'sid=fixture-sid-1; theme=fixture-dark' },
    responseHeaders: {
      'Content-Type': 'text/plain',
      'Set-Cookie': 'fresh=fixture-fresh; Path=/; Secure',
    },
  },
  {
    url: 'https://seven.sliccy.ai/api/session',
    requestHeaders: { Cookie: 'seven=fixture-seven-secret; sid=fixture-seven-sid' },
    responseHeaders: { 'Set-Cookie': 'seven=fixture-seven-rotated; Domain=sliccy.ai' },
  },
];

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
    const home = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-cookies-'));
    return {
      run: (args) => nativeRun(args, home, fake.url),
      saved: async () => {
        const files = await readdir(home, { recursive: true });
        const file = files.find((name) => name.includes('sessions') && name.endsWith('.json'));
        return readFile(path.join(home, file), 'utf8');
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
    return {
      run: (args) => runner.playwright(args, cwd),
      saved: async () =>
        new TextDecoder().decode(await runner.readFile(kernelSessionFile('/root', cwd))),
      close: () => runner.close(),
    };
  },
};

async function withBrowser(kind, body, options = {}) {
  const fake = await fakeCdp({ cookies: jar, traffic, ...options });
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
    await body({ fake, run, ok, refused, saved: runner.saved });
    for (const text of seen) {
      for (const secret of secrets) assert.ok(!text.includes(secret), `${secret} leaked: ${text}`);
    }
    for (const call of fake.calls) {
      if (!/^Network\.(setCookie|deleteCookies)$/.test(call.method)) continue;
      assert.ok(!/sliccy\.ai$/.test(call.params.domain ?? ''), JSON.stringify(call));
    }
    assert.ok(
      fake.calls.every(
        (call) => !['Network.clearBrowserCookies', 'Storage.clearCookies'].includes(call.method)
      )
    );
  } finally {
    await runner.close();
    await fake.close();
  }
}

function count(fake, method) {
  return fake.calls.filter((call) => call.method === method).length;
}

const code = (line) => `### Ran Playwright code\n\`\`\`js\n${line}\n\`\`\`\n`;

for (const kind of ['native', 'wasi']) {
  for (const attachEvents of [false, true]) {
    test(`${kind}: error-page recovery preserves the cookie boundary${attachEvents ? ' with attach events' : ''}`, {
      timeout: 120_000,
    }, async () => {
      await withBrowser(
        kind,
        async ({ fake, run, ok, saved }) => {
          const start = 'https://shop.example.test/app';
          await ok(['open', start]);
          const [tab] = fake.targets;
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
            entries: [start, failed].map((url, index) => ({
              id: index + 1,
              url,
              userTypedURL: url,
              title: url,
              transitionType: 'typed',
            })),
          };
          const reads = count(fake, 'Network.getAllCookies');
          for (const args of [
            ['cookie-list'],
            ['cookie-get', 'sid'],
            ['cookie-clear'],
            ['cookie-set', 'probe', 'fixture-value', '--domain', 'shop.example.test'],
            ['request-headers', '1'],
            ['console'],
          ]) {
            const result = await run(args);
            assert.equal(result.code, 1, result.stdout);
            assert.equal(result.stdout, '');
            assert.ok(result.stderr.startsWith('slicc: refused:'), result.stderr);
            assert.ok(!result.stderr.includes('fixture-error-secret'), result.stderr);
          }
          assert.equal(count(fake, 'Network.getAllCookies'), reads);
          assert.equal(count(fake, 'Network.setCookie'), 0);
          assert.equal(count(fake, 'Network.deleteCookies'), 0);
          assert.equal(await ok(['reload']), 'Reloaded\n');
          assert.equal(count(fake, 'Network.getAllCookies'), reads);
          await ok(['goto', 'https://shop.example.test/recovered', '--tab', tab.targetId]);
          assert.ok((await ok(['cookie-get', 'sid'])).includes('sid=fixture-sid-1'));
          assert.ok((await ok(['request-headers', '1'])).includes('sid=fixture-sid-1'));
          const stored = await saved();
          for (const value of [
            'fixture-sid-1',
            'fixture-dark',
            'fixture-error-secret',
            ...secrets,
          ]) {
            assert.ok(!stored.includes(value), stored);
          }
          tab.url = 'https://vault.sliccy.ai';
          tab.frame = {
            id: tab.targetId,
            loaderId: 'LREAL',
            url: 'https://vault.sliccy.ai/landing',
            securityOrigin: 'https://vault.sliccy.ai',
          };
          const writes = count(fake, 'Network.setCookie');
          const result = await run([
            'cookie-set',
            'probe',
            'fixture-value',
            '--domain',
            'shop.example.test',
          ]);
          assert.equal(result.code, 1, result.stdout);
          assert.equal(result.stdout, '');
          assert.ok(result.stderr.startsWith('slicc: refused:'), result.stderr);
          assert.equal(count(fake, 'Network.setCookie'), writes);
        },
        { attachEvents }
      );
    });
  }

  test(`${kind}: saved headers contain only cookie scopes and output uses current jar values`, {
    timeout: 120_000,
  }, async () => {
    await withBrowser(
      kind,
      async ({ fake, ok, saved }) => {
        await ok(['open', 'https://shop.example.test/app']);
        const first = await saved();
        for (const secret of [
          'fixture-sid-1',
          'fixture-dark',
          'fixture-fresh',
          'fixture-auth',
          'fixture-token',
        ]) {
          assert.ok(!first.includes(secret), first);
        }
        const requests = Object.values(JSON.parse(first).requests).flat();
        assert.deepEqual(requests[0].cookieScopes, [
          { header: 'cookie', name: 'sid', domain: 'shop.example.test', path: '/' },
          { header: 'cookie', name: 'theme', domain: '.example.test', path: '/app' },
          { header: 'set-cookie', name: 'fresh', domain: 'shop.example.test', path: '/' },
        ]);
        fake.jar.find((item) => item.name === 'sid' && item.domain === 'shop.example.test').value =
          'fixture-current';
        fake.jar.splice(
          fake.jar.findIndex((item) => item.name === 'theme'),
          1
        );
        const output = await ok(['request-headers', '1']);
        assert.ok(output.includes('sid=fixture-current; theme=[redacted]'), output);
        assert.ok(output.includes(currentValues.trim()), output);
        assert.ok(!output.includes('fixture-sid-1'), output);
        const after = await saved();
        for (const secret of [
          'fixture-current',
          'fixture-sid-1',
          'fixture-dark',
          'fixture-fresh',
          'fixture-auth',
          'fixture-token',
        ]) {
          assert.ok(!after.includes(secret), after);
        }
      },
      {
        cookies: [...jar, cookie('fresh', 'fixture-fresh', 'shop.example.test')],
        traffic: [
          {
            url: 'https://shop.example.test/api?access_token=fixture-token',
            requestHeaders: {
              Cookie: 'sid=fixture-sid-1; theme=fixture-dark',
              Authorization: 'Bearer fixture-auth',
            },
            responseHeaders: { 'Set-Cookie': 'fresh=fixture-fresh; Path=/' },
          },
        ],
      }
    );
  });

  test(`${kind}: missing historical scopes use only current cookies applicable to an ordinary URL`, {
    timeout: 120_000,
  }, async () => {
    await withBrowser(
      kind,
      async ({ ok, saved }) => {
        await ok(['open', 'http://127.0.0.1:9944/app']);
        const output = await ok(['request-headers', '1']);
        assert.equal(
          output,
          `cookie: session=fixture-current; flavor=fixture-domain\n${currentValues}`
        );
        const text = await saved();
        for (const value of [
          'fixture-current',
          'fixture-domain',
          'fixture-other',
          'fixture-secure',
        ]) {
          assert.ok(!text.includes(value), text);
        }
      },
      {
        cookies: [
          cookie('session', 'fixture-current', '127.0.0.1', { secure: false, path: '/api' }),
          cookie('flavor', 'fixture-domain', '.127.0.0.1', { secure: false }),
          cookie('other', 'fixture-other', 'other.example.test', { secure: false }),
          cookie('path', 'fixture-other', '127.0.0.1', { secure: false, path: '/other' }),
          cookie('secure', 'fixture-secure', '127.0.0.1'),
          ...jar.filter((item) => /sliccy/.test(item.domain)),
        ],
        traffic: [
          {
            url: 'http://127.0.0.1:9944/api/me',
            requestHeaders: { Cookie: '[redacted]' },
            responseHeaders: {},
          },
        ],
      }
    );
  });

  test(`${kind}: missing scopes never fall back on a URL that can share protected parent cookies`, {
    timeout: 120_000,
  }, async () => {
    await withBrowser(
      kind,
      async ({ fake, ok }) => {
        await ok(['open', 'https://shop.example.ai/app']);
        const reads = count(fake, 'Network.getAllCookies');
        assert.equal(await ok(['request-headers', '1']), 'cookie: [redacted]\n');
        assert.equal(count(fake, 'Network.getAllCookies'), reads);
      },
      {
        cookies: [cookie('ordinary', 'fixture-current', 'shop.example.ai')],
        traffic: [
          {
            url: 'https://shop.example.ai/api',
            requestHeaders: { Cookie: '[redacted]' },
            responseHeaders: {},
          },
        ],
      }
    );
  });

  test(`${kind}: removed ordinary cookies retain only their scopes from CDP metadata`, {
    timeout: 120_000,
  }, async () => {
    await withBrowser(
      kind,
      async ({ ok, saved }) => {
        await ok(['open', 'https://shop.example.test/app']);
        const output = await ok(['request-headers', '1']);
        assert.equal(output, `cookie: sid=fixture-sid-1; old=[redacted]\n${currentValues}`);
        const text = await saved();
        assert.ok(!text.includes('fixture-old'), text);
        assert.ok(!text.includes('fixture-sid-1'), text);
        const requests = Object.values(JSON.parse(text).requests).flat();
        assert.deepEqual(requests[0].cookieScopes[1], {
          header: 'cookie',
          name: 'old',
          domain: 'shop.example.test',
          path: '/',
        });
      },
      {
        traffic: [
          {
            url: 'https://shop.example.test/api',
            requestHeaders: { Cookie: 'sid=fixture-sid-1; old=fixture-old' },
            responseHeaders: {},
            associatedCookies: [
              { cookie: cookie('old', 'fixture-old', 'shop.example.test'), blockedReasons: [] },
            ],
          },
        ],
      }
    );
  });

  test(`${kind}: IPv4 cookie scopes are canonical hosts, not parent-domain walks`, {
    timeout: 120_000,
  }, async () => {
    await withBrowser(
      kind,
      async ({ ok, saved }) => {
        await ok(['open', 'http://127.0.0.1:9944/app']);
        assert.equal(
          await ok(['request-headers', '1']),
          `cookie: session=fixture-ip; flavor=fixture-ip-domain\n${currentValues}`
        );
        const text = await saved();
        assert.ok(!text.includes('fixture-ip'), text);
      },
      {
        cookies: [
          cookie('session', 'fixture-ip', '127.0.0.1'),
          cookie('flavor', 'fixture-ip-domain', '.127.0.0.1'),
        ],
        traffic: [
          {
            url: 'http://127.0.0.1:9944/api/me',
            requestHeaders: { Cookie: 'session=fixture-ip; flavor=fixture-ip-domain' },
            responseHeaders: {},
          },
        ],
      }
    );
  });

  test(`${kind}: review regression refuses noncanonical protected or invalid cookie domains`, {
    timeout: 120_000,
  }, async () => {
    await withBrowser(kind, async ({ fake, ok, refused }) => {
      await ok(['open', 'https://shop.example.test/app']);
      const reads = count(fake, 'Network.getAllCookies');
      for (const domain of [
        'sliccy.ai:443',
        '..sliccy.ai',
        'sliccy.ai..',
        'SEVEN。SLICCY。AI',
        'ＳＬＩＣＣＹ．ＡＩ',
        'seven｡sliccy｡ai',
        '.AI.',
        'ai:80',
        'sliccy..ai',
        'ordinary..test',
        '',
        '.',
        'user@sliccy.ai',
        'sliccy.ai/path',
        'example.test@sliccy.ai',
        'sliccy.ai@example.test',
        'example.test/path',
        'example.test?x=sliccy.ai',
        'example.test#sliccy.ai',
        ' example.test ',
        'xn--.test',
        'sliccy.ai:bogus',
        'https://sliccy.ai',
      ]) {
        assert.equal(
          await refused(['cookie-set', 'probe', 'fixture-value', '--domain', domain]),
          refusal
        );
        assert.equal(await refused(['cookie-list', '--domain', domain]), refusal);
      }
      assert.equal(count(fake, 'Network.setCookie'), 0);
      assert.equal(count(fake, 'Network.getAllCookies'), reads);
      for (const domain of [
        'EXAMPLE.TEST.',
        '.example.test',
        '..example.test..',
        'example.test:443',
        'ＥＸＡＭＰＬＥ．ＴＥＳＴ',
        'bücher.test',
        'xn--bcher-kva.test',
      ]) {
        await ok(['cookie-set', 'ordinary', 'fixture-value', '--domain', domain]);
      }
      const writes = count(fake, 'Network.setCookie');
      await ok(['open', 'https://sliccy.ai../app']);
      assert.equal(await refused(['cookie-set', 'probe', 'fixture-value']), refusal);
      assert.equal(count(fake, 'Network.setCookie'), writes);
    });
  });

  test(`${kind}: review regression hides protected cookie scopes without hiding ordinary host-only cookies`, {
    timeout: 120_000,
  }, async () => {
    const parent = cookie('shared', 'fixture-parent-scope', '.ai');
    const ordinary = cookie('shared', 'fixture-host-only', 'shop.example.ai');
    await withBrowser(
      kind,
      async ({ ok, saved }) => {
        await ok(['open', 'https://shop.example.ai/app']);
        assert.equal(await ok(['request-headers', '1']), 'cookie: [redacted]\n');
        assert.equal(
          await ok(['request-headers', '2']),
          `cookie: shared=fixture-host-only\n${currentValues}`
        );
        assert.equal(await ok(['request-headers', '7']), 'cookie: [redacted]\n');
        assert.equal(await ok(['request-headers', '8']), 'cookie: [redacted]\n');
        assert.equal(
          await ok(['response-headers', '2']),
          `set-cookie: fresh=fixture-host-fresh; Domain=shop.example.ai; Path=/\n${currentValues}`
        );
        for (const index of [1, 3, 4, 5, 6]) {
          assert.equal(await ok(['response-headers', String(index)]), 'set-cookie: [redacted]\n');
        }
        for (const index of [1, 3, 4, 5, 6]) {
          const detail = await ok(['request', String(index)]);
          assert.ok(detail.includes('set-cookie: [redacted]'), detail);
        }
        const stored = await saved();
        for (const value of [
          'fixture-parent-scope',
          'fixture-parent-new',
          'fixture-explicit',
          'fixture-multi',
          'fixture-idn',
          'fixture-invalid',
          'fixture-deleted',
          'fixture-unknown',
        ]) {
          assert.ok(!stored.includes(value), stored);
        }
        assert.ok(!stored.includes('fixture-host-only'), stored);
        assert.ok(!stored.includes('fixture-host-fresh'), stored);
      },
      {
        cookies: [
          ...jar,
          parent,
          ordinary,
          cookie('fresh', 'fixture-host-fresh', 'shop.example.ai'),
        ],
        traffic: [
          {
            url: 'https://shop.example.ai/parent',
            requestHeaders: { Cookie: 'shared=fixture-parent-scope' },
            responseHeaders: { 'Set-Cookie': 'shared=fixture-parent-new; Domain=.ai' },
            associatedCookies: [{ cookie: parent, blockedReasons: [] }],
          },
          {
            url: 'https://shop.example.ai/ordinary',
            requestHeaders: { Cookie: 'shared=fixture-host-only' },
            responseHeaders: { 'Set-Cookie': 'fresh=fixture-host-fresh; Path=/' },
            associatedCookies: [{ cookie: ordinary, blockedReasons: [] }],
          },
          {
            url: 'https://shop.example.test/explicit',
            requestHeaders: {},
            responseHeaders: { 'Set-Cookie': 'shared=fixture-explicit; dOmAiN=.sliccy.ai' },
          },
          {
            url: 'https://shop.example.test/multi',
            requestHeaders: {},
            responseHeaders: {
              'Set-Cookie':
                'fresh=fixture-normal\nshared=fixture-multi; Domain=ai\nother=fixture-other',
            },
          },
          {
            url: 'https://shop.example.test/idn',
            requestHeaders: {},
            responseHeaders: { 'Set-Cookie': 'shared=fixture-idn; Domain=SEVEN。SLICCY。AI' },
          },
          {
            url: 'https://shop.example.test/invalid',
            requestHeaders: {},
            responseHeaders: { 'Set-Cookie': 'shared=fixture-invalid; Domain=ordinary..test' },
          },
          {
            url: 'https://shop.example.ai/deleted',
            requestHeaders: { Cookie: 'old=fixture-deleted' },
            responseHeaders: {},
            associatedCookies: [
              { cookie: cookie('old', 'fixture-deleted', '.ai'), blockedReasons: [] },
            ],
          },
          {
            url: 'https://shop.example.ai/unknown',
            requestHeaders: { Cookie: 'unknown=fixture-unknown' },
            responseHeaders: {},
          },
        ],
      }
    );
  });

  test(`${kind}: review regression never persists console output containing protected cookie values`, {
    timeout: 120_000,
  }, async () => {
    await withBrowser(
      kind,
      async ({ ok, saved }) => {
        await ok(['open', 'https://shop.example.ai/app']);
        const text = await ok(['console']);
        assert.ok(text.includes('kept: cookie banner shown'), text);
        assert.ok(text.includes('document.cookie = theme=fixture-dark'), text);
        assert.ok(text.includes('shared=fixture-host-only'), text);
        assert.ok(!text.includes('fixture-parent-scope'), text);
        assert.ok(!text.includes('fixture-quoted'), text);
        assert.ok(!text.includes('fixture-rotated'), text);
        const stored = await saved();
        assert.ok(!stored.includes('fixture-parent-scope'), stored);
        assert.ok(!stored.includes('fixture-quoted'), stored);
        assert.ok(!stored.includes('fixture-rotated'), stored);
        assert.ok(!stored.includes('document.cookie = unknown=fixture-unknown'), stored);
        assert.ok(!stored.includes('fixture-dark'), stored);
        assert.ok(!stored.includes('fixture-host-only'), stored);
      },
      {
        cookies: [
          ...jar,
          cookie('shared', 'fixture-parent-scope', '.ai'),
          cookie('shared', 'fixture-host-only', 'shop.example.ai'),
          cookie('quoted', 'fixture-quoted"\\☃', '.ai'),
          cookie('rotated', 'fixture-rotated', '.ai'),
        ],
        traffic: [
          {
            url: 'https://shop.example.ai/rotate',
            requestHeaders: {},
            responseHeaders: { 'Set-Cookie': 'rotated=fixture-rotated; Domain=.ai' },
          },
        ],
        consoleLines: [
          { text: 'document.cookie = shared=fixture-parent-scope' },
          { text: 'shared=fixture-parent-scope', url: 'https://cdn.example.test/app.js' },
          { text: 'fixture-parent-scope', url: 'https://cdn.example.test/app.js' },
          { text: 'document.cookie = unknown=fixture-unknown' },
          { text: 'kept: cookie banner shown' },
          { text: 'document.cookie = theme=fixture-dark', url: 'https://shop.example.test/app.js' },
          { text: 'shared=fixture-host-only' },
          { text: 'fixture-quoted"\\☃', url: 'https://cdn.example.test/app.js' },
          { text: 'fixture-rotated', url: 'https://cdn.example.test/app.js' },
        ],
      }
    );
  });

  test(`${kind}: cookie help pages match upstream playwright-cli 0.1.22`, {
    timeout: 120_000,
  }, async () => {
    await withBrowser(kind, async ({ ok }) => {
      assert.equal(
        await ok(['cookie-list', '--help']),
        'playwright-cli cookie-list \n\nList all cookies (optionally filtered by domain/path)\n\nOptions:\n  --domain                    filter cookies by domain\n  --path                      filter cookies by path\n'
      );
      assert.equal(
        await ok(['cookie-get', '--help']),
        'playwright-cli cookie-get <name>\n\nGet a specific cookie by name\n\nArguments:\n  <name>                      cookie name\n'
      );
      assert.equal(
        await ok(['cookie-set', '--help']),
        'playwright-cli cookie-set <name> <value>\n\nSet a cookie with optional flags\n\nArguments:\n  <name>                      cookie name\n  <value>                     cookie value\nOptions:\n  --domain                    cookie domain\n  --path                      cookie path\n  --expires                   cookie expiration as unix timestamp\n  --httpOnly                  whether the cookie is http only\n  --secure                    whether the cookie is secure\n  --sameSite                  cookie samesite attribute (one of: Strict, Lax, None)\n'
      );
      assert.equal(
        await ok(['cookie-delete', '--help']),
        'playwright-cli cookie-delete <name>\n\nDelete a specific cookie\n\nArguments:\n  <name>                      cookie name\n'
      );
      assert.equal(
        await ok(['cookie-clear', '--help']),
        'playwright-cli cookie-clear \n\nClear all cookies\n\n'
      );
      const global = await ok(['--help']);
      assert.ok(
        global.includes(
          'Storage:\n  cookie-list                 list all cookies (optionally filtered by domain/path)\n  cookie-get <name>           get a specific cookie by name\n  cookie-set <name> <value>   set a cookie with optional flags\n  cookie-delete <name>        delete a specific cookie\n  cookie-clear                clear all cookies\n'
        ),
        global
      );
      assert.ok(!global.split('Not implemented yet:')[1].includes('cookie-'), global);
    });
  });

  test(`${kind}: cookie-list and cookie-get print ordinary cookies and never protected ones`, {
    timeout: 120_000,
  }, async () => {
    await withBrowser(kind, async ({ fake, ok, refused }) => {
      await ok(['open', 'https://shop.example.test/app']);
      assert.equal(
        await ok(['cookie-list']),
        'sid=fixture-sid-1 (domain: shop.example.test, path: /)\ntheme=fixture-dark (domain: .example.test, path: /app)\n'
      );
      assert.equal(
        await ok(['cookie-list', '--domain', 'shop']),
        'sid=fixture-sid-1 (domain: shop.example.test, path: /)\n'
      );
      assert.equal(
        await ok(['cookie-list', '--path=/app']),
        'theme=fixture-dark (domain: .example.test, path: /app)\n'
      );
      assert.equal(await ok(['cookie-list', '--domain', 'nowhere']), 'No cookies found\n');
      assert.equal(await ok(['cookie-list', '--domain', 'slicc']), 'No cookies found\n');
      const reads = count(fake, 'Network.getAllCookies');
      for (const domain of ['sliccy.ai', '.sliccy.ai', 'vault.sliccy.ai', 'seven.sliccy.ai']) {
        assert.equal(await refused(['cookie-list', '--domain', domain]), refusal);
      }
      assert.equal(count(fake, 'Network.getAllCookies'), reads);
      assert.equal(
        await ok(['cookie-get', 'sid']),
        'sid=fixture-sid-1 (domain: shop.example.test, path: /, httpOnly: true, secure: true, sameSite: Strict)\n'
      );
      assert.equal(
        await ok(['cookie-get', 'theme']),
        'theme=fixture-dark (domain: .example.test, path: /app, httpOnly: false, secure: false, sameSite: Lax)\n'
      );
      for (const name of ['seven', 'vault', 'chip', 'missing']) {
        assert.equal(await ok(['cookie-get', name]), `Cookie '${name}' not found\n`);
      }
    });
  });

  test(`${kind}: cookie-set, cookie-delete and cookie-clear touch only ordinary cookies`, {
    timeout: 120_000,
  }, async () => {
    await withBrowser(kind, async ({ fake, run, ok, refused }) => {
      await ok(['open', 'https://shop.example.test/app']);
      assert.equal(
        await ok(['cookie-set', 'token', 'fixture-token']),
        code(
          'await page.context().addCookies([{"name":"token","value":"fixture-token","domain":"shop.example.test","path":"/"}]);'
        )
      );
      assert.equal(
        await ok([
          'cookie-set',
          'pref',
          'fixture-pref',
          '--domain',
          'example.test',
          '--path=/app',
          '--expires',
          '1924992000',
          '--httpOnly',
          '--secure',
          '--sameSite',
          'None',
        ]),
        code(
          'await page.context().addCookies([{"name":"pref","value":"fixture-pref","domain":"example.test","path":"/app","expires":1924992000,"httpOnly":true,"secure":true,"sameSite":"None"}]);'
        )
      );
      const set = fake.calls.filter((call) => call.method === 'Network.setCookie');
      assert.deepEqual(
        set.map((call) => call.params),
        [
          { name: 'token', value: 'fixture-token', domain: 'shop.example.test', path: '/' },
          {
            name: 'pref',
            value: 'fixture-pref',
            domain: 'example.test',
            path: '/app',
            expires: 1924992000,
            httpOnly: true,
            secure: true,
            sameSite: 'None',
          },
        ]
      );
      assert.equal(
        await ok(['cookie-get', 'pref']),
        'pref=fixture-pref (domain: .example.test, path: /app, httpOnly: true, secure: true, sameSite: None)\n'
      );
      for (const domain of ['sliccy.ai', '.sliccy.ai', 'vault.sliccy.ai', 'x.sliccy.ai', 'ai']) {
        assert.equal(
          await refused(['cookie-set', 'seven', 'fixture-forged', '--domain', domain]),
          refusal
        );
      }
      assert.equal(count(fake, 'Network.setCookie'), 2);
      const bad = await run(['cookie-set', 'x', 'y', '--sameSite', 'Bogus']);
      assert.equal(bad.code, 1);
      assert.equal(
        bad.stderr,
        `error: '--sameSite' option: Invalid option: expected one of "Strict"|"Lax"|"None"\n`
      );
      const soon = await run(['cookie-set', 'x', 'y', '--expires', 'soon']);
      assert.equal(soon.code, 1);
      assert.equal(soon.stderr, "error: '--expires' option: expected number, received 'soon'\n");
      const missing = await run(['cookie-set', 'x']);
      assert.equal(missing.code, 1);
      assert.equal(
        missing.stderr,
        "error: 'value' argument: expected string, received undefined\n"
      );
      assert.equal(
        await ok(['cookie-delete', 'sid']),
        code("await page.context().clearCookies({ name: 'sid' });")
      );
      assert.deepEqual(
        fake.calls
          .filter((call) => call.method === 'Network.deleteCookies')
          .map((call) => call.params),
        [{ name: 'sid', domain: 'shop.example.test', path: '/' }]
      );
      assert.ok(fake.jar.some((item) => item.value === 'fixture-seven-sid'));
      assert.equal(await ok(['cookie-get', 'sid']), "Cookie 'sid' not found\n");
      assert.equal(
        await ok(['cookie-delete', 'seven']),
        code("await page.context().clearCookies({ name: 'seven' });")
      );
      assert.equal(await ok(['cookie-clear']), code('await page.context().clearCookies();'));
      assert.equal(await ok(['cookie-list']), 'No cookies found\n');
      assert.deepEqual(fake.jar.map((item) => item.value).sort(), [
        'fixture-chip-secret',
        'fixture-seven-secret',
        'fixture-seven-sid',
        'fixture-vault-secret',
      ]);
    });
  });

  test(`${kind}: request headers show ordinary cookie values and hide protected requests`, {
    timeout: 120_000,
  }, async () => {
    await withBrowser(
      kind,
      async ({ ok }) => {
        await ok(['open', 'https://shop.example.test/app']);
        const listed = await ok(['requests']);
        assert.ok(listed.includes('https://shop.example.test/api/me'), listed);
        assert.ok(!listed.includes('sliccy'), listed);
        assert.equal(
          await ok(['request-headers', '1']),
          `accept: */*\ncookie: sid=fixture-sid-1; theme=fixture-dark\n${currentValues}`
        );
        assert.equal(
          await ok(['response-headers', '1']),
          `content-type: text/plain\nset-cookie: fresh=fixture-fresh; Domain=shop.example.test; Path=/\n${currentValues}`
        );
        const detail = await ok(['request', '1']);
        assert.ok(detail.includes('    cookie: sid=fixture-sid-1; theme=fixture-dark'), detail);
        assert.ok(
          detail.includes('    set-cookie: fresh=fixture-fresh; Domain=shop.example.test; Path=/'),
          detail
        );
        assert.ok(detail.includes(currentValues.trim()), detail);
        assert.ok(!detail.includes('[redacted]'), detail);
      },
      { cookies: [...jar, cookie('fresh', 'fixture-fresh', 'shop.example.test')] }
    );
  });
}
