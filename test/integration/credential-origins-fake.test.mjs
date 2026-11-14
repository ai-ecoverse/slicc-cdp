import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
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
const credentialRefusal = 'slicc: refused: this origin holds sign-in credentials\n';
const idp = 'https://ims-na1.adobelogin.com';
const shop = 'https://shop.example.test';
const secrets = [
  'fixture-idp-host',
  'fixture-idp-parent',
  'fixture-idp-stage',
  'fixture-idp-chip',
  'fixture-idp-request',
  'fixture-idp-response',
  'fixture-idp-local',
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
  cookie('sid', 'fixture-shop-sid', 'shop.example.test'),
  cookie('host', 'fixture-idp-host', 'ims-na1.adobelogin.com', { httpOnly: true }),
  cookie('parent', 'fixture-idp-parent', '.adobelogin.com'),
  cookie('stage', 'fixture-idp-stage', 'ims-na1-stg1.adobelogin.com'),
  cookie('chip', 'fixture-idp-chip', 'widget.example.test', {
    partitionKey: { topLevelSite: 'https://adobelogin.com', hasCrossSiteAncestor: true },
  }),
  cookie('lookalike', 'fixture-lookalike', 'evil-adobelogin.com'),
  cookie('adobe', 'fixture-adobe', '.adobe.com'),
  cookie('auth', 'fixture-adobe-auth', 'auth.services.adobe.com'),
];

const traffic = [
  {
    url: `${idp}/ims/check/v6/token`,
    requestHeaders: { Cookie: 'host=fixture-idp-request; parent=fixture-idp-parent' },
    responseHeaders: { 'Set-Cookie': 'host=fixture-idp-response; Domain=adobelogin.com' },
  },
  {
    url: `${shop}/api/me`,
    requestHeaders: { Cookie: 'sid=fixture-shop-sid' },
    responseHeaders: { 'Content-Type': 'text/plain' },
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
    const home = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-credential-'));
    return {
      run: (args) => nativeRun(args, home, fake.url),
      read: (name) => readFile(path.join(home, name), 'utf8'),
      write: (name, text) => writeFile(path.join(home, name), text),
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
      read: async (name) => new TextDecoder().decode(await runner.read(cwd, name)),
      write: (name, text) => runner.write(cwd, name, text),
      close: () => runner.close(),
    };
  },
};

function storageModel() {
  const local = new Map([
    [shop, new Map([['cart', 'fixture-cart']])],
    [idp, new Map([['token', 'fixture-idp-local']])],
  ]);
  const touched = [];
  const tail = (expression) => expression.slice(expression.lastIndexOf('})(') + 3, -1);
  const evaluate = (target, expression) => {
    const origin = new URL(target.url).origin;
    const request = JSON.parse(tail(expression));
    touched.push({ origin, op: `${request.kind}-${request.op}` });
    if (request.expected !== origin) return { origin };
    if (!local.has(origin)) local.set(origin, new Map());
    const storage = local.get(origin);
    if (request.op === 'list')
      return { origin, items: [...storage].map(([name, value]) => ({ name, value })) };
    if (request.op === 'get') return { origin, value: storage.get(request.key) ?? null };
    if (request.op === 'set') storage.set(request.key, request.value);
    else if (request.op === 'delete') storage.delete(request.key);
    else storage.clear();
    return { origin };
  };
  return { local, touched, evaluate };
}

async function withBrowser(kind, body) {
  const model = storageModel();
  const fake = await fakeCdp({
    cookies: jar,
    traffic,
    intercept: (message, send) => {
      const expression = message.params?.expression ?? '';
      if (
        message.method !== 'Runtime.evaluate' ||
        !/storage\.getItem\(request\.key\)/.test(expression)
      )
        return false;
      const id = fake.calls.at(-1)?.target;
      const target = fake.targets.find((item) => item.targetId === id);
      send({
        id: message.id,
        result: { result: { type: 'object', value: model.evaluate(target, expression) } },
        ...(message.sessionId ? { sessionId: message.sessionId } : {}),
      });
      return true;
    },
  });
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
    for (const text of seen) {
      for (const secret of secrets) assert.ok(!text.includes(secret), `${secret} leaked: ${text}`);
    }
    for (const call of fake.calls) {
      if (!/^Network\.(setCookie|deleteCookies)$/.test(call.method)) continue;
      const domain = (call.params.domain ?? '').replace(/^\./, '');
      assert.ok(
        !['adobelogin.com', 'ims-na1.adobelogin.com', 'ims-na1-stg1.adobelogin.com'].includes(
          domain
        ),
        JSON.stringify(call)
      );
    }
    assert.deepEqual([...model.local.get(idp)], [['token', 'fixture-idp-local']]);
    assert.ok(
      model.touched.every((entry) => entry.origin !== idp),
      JSON.stringify(model.touched)
    );
  } finally {
    await runner.close();
    await fake.close();
  }
}

for (const kind of ['native', 'wasi']) {
  test(`${kind}: credential-origin cookies are withheld from cookie commands, state-save and request headers`, {
    timeout: 120_000,
  }, async () => {
    await withBrowser(kind, async ({ fake, runner, ok, refused }) => {
      await ok(['open', `${shop}/app`]);
      const listed = await ok(['cookie-list']);
      assert.ok(listed.includes('sid=fixture-shop-sid'), listed);
      assert.ok(listed.includes('lookalike=fixture-lookalike'), listed);
      assert.ok(listed.includes('adobe=fixture-adobe (domain: .adobe.com'), listed);
      assert.ok(listed.includes('auth=fixture-adobe-auth'), listed);
      assert.equal(await ok(['cookie-get', 'host']), "Cookie 'host' not found\n");
      assert.equal(await ok(['cookie-get', 'parent']), "Cookie 'parent' not found\n");
      for (const domain of [
        '.adobelogin.com',
        'adobelogin.com',
        'ims-na1.adobelogin.com',
        'IMS-NA1-STG1.ADOBELOGIN.COM',
      ]) {
        assert.equal(
          await refused(['cookie-set', 'probe', 'fixture-value', '--domain', domain]),
          credentialRefusal,
          domain
        );
        assert.equal(await refused(['cookie-list', '--domain', domain]), credentialRefusal, domain);
      }
      await ok(['cookie-set', 'probe', 'fixture-value', '--domain', 'x.ims-na1.adobelogin.com']);
      await ok(['cookie-set', 'probe', 'fixture-value', '--domain', '.adobe.com']);
      await ok(['cookie-set', 'probe', 'fixture-value', '--domain', 'auth.services.adobe.com']);
      await ok(['cookie-delete', 'parent']);
      await ok(['cookie-clear']);
      assert.ok(fake.jar.some((item) => item.value === 'fixture-idp-host'));
      assert.ok(fake.jar.some((item) => item.value === 'fixture-idp-parent'));
      assert.ok(!fake.jar.some((item) => item.value === 'fixture-adobe'));
      assert.ok(!fake.jar.some((item) => item.value === 'fixture-shop-sid'));
      const headers = await ok(['request-headers', '1']);
      assert.ok(!headers.includes('fixture-idp'), headers);
      const ordinary = await ok(['request-headers', '2']);
      assert.ok(ordinary.includes('sid='), ordinary);
      const response = await ok(['response-headers', '1']);
      assert.ok(!response.includes('fixture-idp'), response);
      fake.jar.push(cookie('again', 'fixture-shop-again', 'shop.example.test'));
      await ok(['state-save', 'state.json']);
      const saved = await runner.read('state.json');
      assert.ok(saved.includes('fixture-shop-again'), saved);
      assert.ok(
        !JSON.parse(saved).cookies.some((item) => /(^|\.)adobelogin\.com$/.test(item.domain)),
        saved
      );
      await runner.write(
        'idp-cookie.json',
        JSON.stringify({
          cookies: [{ name: 'x', value: 'y', domain: '.adobelogin.com', path: '/' }],
          origins: [],
        })
      );
      assert.equal(await refused(['state-load', 'idp-cookie.json']), credentialRefusal);
      await runner.write(
        'idp-origin.json',
        JSON.stringify({
          cookies: [],
          origins: [{ origin: idp, localStorage: [{ name: 'token', value: 'planted' }] }],
        })
      );
      assert.equal(await refused(['state-load', 'idp-origin.json']), credentialRefusal);
    });
  });

  test(`${kind}: an IdP page stays drivable while its web storage is withheld`, {
    timeout: 120_000,
  }, async () => {
    await withBrowser(kind, async ({ fake, runner, ok, refused }) => {
      await ok(['open', `${idp}/ims/authorize/v2?client_id=public`]);
      const tab = fake.targets.at(-1);
      assert.equal(tab.url, `${idp}/ims/authorize/v2?client_id=public`);
      assert.equal(await ok(['eval', 'document.title']), 'fake-eval\n');
      assert.match(await ok(['snapshot']), /button "Go"/);
      await ok(['click', 'e2']);
      await ok(['type', 'user@example.test']);
      await ok(['press', 'Enter']);
      for (const args of [
        ['localstorage-list'],
        ['localstorage-get', 'token'],
        ['localstorage-set', 'token', 'planted'],
        ['localstorage-clear'],
        ['sessionstorage-list'],
        ['sessionstorage-set', 'step', '2'],
      ])
        assert.equal(await refused(args), credentialRefusal, args.join(' '));
      await ok(['goto', `${shop}/app`]);
      assert.equal(await ok(['localstorage-list']), 'cart=fixture-cart\n');
      await ok(['goto', `${idp}/ims/authorize/v2`]);
      await ok(['state-save', 'state.json']);
      const saved = JSON.parse(await runner.read('state.json'));
      assert.deepEqual(saved.origins, []);
      assert.ok(
        !saved.cookies.some((item) => /(^|\.)adobelogin\.com$/.test(item.domain)),
        JSON.stringify(saved)
      );
    });
  });
}
