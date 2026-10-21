import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { fakeCdp } from './fake-cdp.mjs';
import { HARNESS_PORTS } from './ports.mjs';
import { kernelRunner } from './runners.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const native = path.join(
  root,
  'target/debug',
  process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'
);
const wasm = path.join(root, 'target/wasm32-wasip1/debug');
const refusal = "slicc: refused: cookies on seven's origin\n";
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
    return { run: (args) => runner.playwright(args, cwd), close: () => runner.close() };
  },
};

async function withBrowser(kind, body) {
  const fake = await fakeCdp({ cookies: jar, traffic });
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
    await body({ fake, run, ok, refused });
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
    await withBrowser(kind, async ({ ok }) => {
      await ok(['open', 'https://shop.example.test/app']);
      const listed = await ok(['requests']);
      assert.ok(listed.includes('https://shop.example.test/api/me'), listed);
      assert.ok(!listed.includes('sliccy'), listed);
      assert.equal(
        await ok(['request-headers', '1']),
        'accept: */*\ncookie: sid=fixture-sid-1; theme=fixture-dark\n'
      );
      assert.equal(
        await ok(['response-headers', '1']),
        'content-type: text/plain\nset-cookie: fresh=fixture-fresh; Path=/; Secure\n'
      );
      const detail = await ok(['request', '1']);
      assert.ok(detail.includes('    cookie: sid=fixture-sid-1; theme=fixture-dark'), detail);
      assert.ok(detail.includes('    set-cookie: fresh=fixture-fresh; Path=/; Secure'), detail);
      assert.ok(!detail.includes('[redacted]'), detail);
    });
  });
}
