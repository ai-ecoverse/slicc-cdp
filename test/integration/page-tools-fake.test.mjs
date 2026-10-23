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
import './watchdog.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const exe =
  process.env.SLICC_PAGE_TOOLS_BIN ||
  path.join(
    root,
    'target/debug',
    process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'
  );
const wasm = (name) => path.join(root, 'target/wasm32-wasip1/debug', `${name}.wasm`);
const conditions = (offline) => ({
  offline,
  latency: 0,
  downloadThroughput: -1,
  uploadThroughput: -1,
});
const linkHeader = [
  '<https://example.test/next?token=LINKSECRET>; rel="next"',
  '<https://seven.sliccy.ai/os/?code=SEVENSECRET>; rel="preload"',
  '<https://vault.sliccy.ai/x>; rel="alternate"',
  '</skills/demo>; rel="https://www.sliccy.ai/rel/upskill"; title="Install the demo"',
].join(', ');
const touching = new Set([
  'Runtime.evaluate',
  'Runtime.callFunctionOn',
  'DOM.resolveNode',
  'DOM.getFrameOwner',
  'Page.createIsolatedWorld',
  'Network.emulateNetworkConditions',
]);
const touched = (fake, target) =>
  fake.calls.filter((call) => call.target === target && touching.has(call.method));

function run(args, home) {
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
    const child = spawn(exe, args, { cwd: home, env, stdio: ['ignore', 'pipe', 'pipe'] });
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

function page(intercept = () => false) {
  return (message, send) =>
    intercept(message, send, (result) =>
      send({ id: message.id, sessionId: message.sessionId, result })
    );
}

async function withFake(intercept, body, extra = []) {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-page-tools-'));
  const fake = await fakeCdp({ intercept: page(intercept) });
  fake.targets.push({ targetId: 'TAB', url: 'https://example.test/' }, ...extra);
  try {
    await body(fake, (args) => run(['--cdp', fake.url, ...args], home), home);
  } finally {
    await fake.close();
    await rm(home, { recursive: true, force: true });
  }
}

async function withKernel(intercept, body) {
  const fake = await fakeCdp({ intercept: page(intercept) });
  fake.targets.push({ targetId: 'TAB', url: 'https://example.test/' });
  let runner;
  try {
    runner = await kernelRunner(wasm('playwright-cli'), wasm('curlwright'), fake.url);
    const cwd = await runner.fresh();
    await body(fake, (args) => runner.playwright(args, cwd), runner, cwd);
  } finally {
    await Promise.resolve(runner?.close()).catch(() => {});
    await fake.close();
  }
}

const sent = (fake, method) => fake.calls.filter((call) => call.method === method);
const pageSession = (fake) =>
  fake.calls.find((call) => call.method === 'Runtime.enable' && call.target === 'TAB')?.sessionId;

function remoteEval(value) {
  return (message, _send, answer) => {
    if (message.method === 'Runtime.evaluate' && message.params.objectGroup === 'slicc-eval') {
      answer({ result: value(message.params) });
      return true;
    }
    return false;
  };
}

function locatorBackend(message, _send, answer) {
  if (
    message.method === 'Runtime.callFunctionOn' &&
    message.params.functionDeclaration.includes('generateSelectorSimple')
  ) {
    const rest = message.params.arguments?.[0]?.value;
    const own = 'internal:role=button[name="Go"i]';
    const selector = rest ? `${own} >> internal:control=enter-frame >> ${rest}` : own;
    answer({
      result: {
        type: 'object',
        value: [selector, rest ? 'nested' : "getByRole('button', { name: 'Go' })"],
      },
    });
    return true;
  }
  if (
    message.method === 'Runtime.callFunctionOn' &&
    message.params.functionDeclaration.includes('x-slicc-highlight')
  ) {
    answer({ result: { type: 'undefined' } });
    return true;
  }
  return false;
}

test('page tools leave the "Not implemented yet" list and print their help pages', async () => {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-page-tools-help-'));
  try {
    const global = await run(['--help'], home);
    assert.equal(global.code, 0, global.stderr);
    const pending = global.stdout.split('Not implemented yet:')[1].split('\n')[1];
    for (const name of ['network-state-set', 'eval-file', 'fetch', 'generate-locator', 'highlight'])
      assert.ok(!pending.split(', ').includes(name.trim()), `${name} still pending`);
    for (const line of [
      '  network-state-set <state>   set the browser network state to online or offline',
      '  generate-locator <target>   generate a playwright locator for the given element',
      '  highlight [target]          show (or with --hide, remove) a highlight overlay for an element; `--hide` without a target hides all page highlights.',
      '  eval-file <path>            evaluate javascript from a local file on page',
      '  fetch <url>                 fetch a url in the tab and list its link headers',
    ])
      assert.ok(global.stdout.includes(`${line}\n`), line);
    const pages = {
      'generate-locator':
        'playwright-cli generate-locator <target>\n\nGenerate a Playwright locator for the given element\n\nArguments:\n  <target>                    exact target element reference from the page snapshot, or a unique element selector\n',
      highlight:
        'playwright-cli highlight [target]\n\nShow (or with --hide, remove) a highlight overlay for an element; `--hide` without a target hides all page highlights.\n\nArguments:\n  [target]                    exact target element reference from the page snapshot, or a unique element selector\nOptions:\n  --hide                      hide a previously added highlight for this element, or all page highlights when no element is given\n  --style                     additional inline css applied to the highlight overlay, e.g. "outline: 2px dashed red"\n',
    };
    for (const [name, text] of Object.entries(pages)) {
      const help = await run([name, '--help'], home);
      assert.equal(help.code, 0, help.stderr);
      assert.equal(help.stdout, text);
    }
    for (const name of ['network-state-set', 'eval-file', 'fetch']) {
      const help = await run([name, '--help'], home);
      assert.equal(help.code, 0, help.stderr);
      assert.match(help.stdout, new RegExp(`^playwright-cli ${name} <`));
      assert.doesNotMatch(help.stdout, /Not implemented/);
    }
    const network = await run(['network-state-set', '--help'], home);
    assert.ok(
      network.stdout.startsWith(
        'playwright-cli network-state-set <state>\n\nSet the browser network state to online or offline\n'
      )
    );
    assert.ok(
      network.stdout.includes(
        '  <state>                     set to "offline" to simulate offline mode, "online" to restore network connectivity\n'
      )
    );
  } finally {
    await rm(home, { recursive: true, force: true });
  }
});

test('network-state-set matches upstream output and arms offline on later attaches only', async () => {
  await withFake(
    remoteEval(() => ({ type: 'number', value: 1 })),
    async (fake, pw) => {
      for (const args of [['network-state-set'], ['network-state-set', 'later']]) {
        const bad = await pw(args);
        assert.equal(bad.code, 1);
        assert.equal(bad.stdout, '');
        assert.equal(
          bad.stderr,
          `error: 'state' argument: Invalid option: expected one of "online"|"offline"\n`
        );
      }
      const tabbed = await pw(['network-state-set', 'offline', '--tab', 'TAB']);
      assert.equal(tabbed.code, 1);
      assert.match(tabbed.stderr, /Unknown option/);
      assert.equal(fake.calls.length, 0);
      const offline = await pw(['network-state-set', 'offline']);
      assert.equal(offline.code, 0, offline.stderr);
      assert.equal(
        offline.stdout,
        '### Result\nNetwork is now offline\n### Ran Playwright code\n```js\nawait page.context().setOffline(true);\n```\n'
      );
      assert.equal(fake.calls.length, 0);
      const evaluated = await pw(['eval', '1']);
      assert.equal(evaluated.code, 0, evaluated.stderr);
      const emulated = sent(fake, 'Network.emulateNetworkConditions');
      assert.equal(emulated.length, 1);
      assert.deepEqual(emulated[0].params, conditions(true));
      assert.equal(emulated[0].target, 'TAB');
      const order = fake.calls.map((call) => call.method);
      assert.ok(
        order.indexOf('Network.emulateNetworkConditions') <
          fake.calls.findIndex((call) => call.params?.objectGroup === 'slicc-eval')
      );
      const online = await pw(['network-state-set', 'online']);
      assert.equal(online.code, 0, online.stderr);
      assert.equal(
        online.stdout,
        '### Result\nNetwork is now online\n### Ran Playwright code\n```js\nawait page.context().setOffline(false);\n```\n'
      );
      const again = await pw(['eval', '1']);
      assert.equal(again.code, 0, again.stderr);
      assert.equal(sent(fake, 'Network.emulateNetworkConditions').length, 1);
    }
  );
});

test('network-state-set never reaches a protected tab', async () => {
  await withFake(
    () => false,
    async (fake, pw) => {
      const offline = await pw(['network-state-set', 'offline']);
      assert.equal(offline.code, 0, offline.stderr);
      const refused = await pw(['eval', '1', '--tab', 'SEVEN']);
      assert.equal(refused.code, 1);
      assert.match(refused.stderr, /^slicc: refused: /);
      assert.deepEqual(touched(fake, 'SEVEN'), []);
    },
    [{ targetId: 'SEVEN', url: 'https://seven.sliccy.ai/os/' }]
  );
});

test('eval-file reads the named file and evaluates it through the eval path', async () => {
  const evaluated = [];
  await withFake(
    remoteEval((params) => {
      evaluated.push(params.expression);
      return { type: 'number', value: 42 };
    }),
    async (fake, pw, home) => {
      const source = 'const answer = 41 + 1;\nanswer\n';
      await writeFile(path.join(home, 'script.js'), source);
      const result = await pw(['eval-file', 'script.js']);
      assert.equal(result.code, 0, result.stderr);
      assert.equal(result.stdout, '42\n');
      assert.deepEqual(evaluated, [source]);
      const saved = await pw(['eval-file', 'script.js', '--output', 'out/result.txt']);
      assert.equal(saved.code, 0, saved.stderr);
      assert.equal(saved.stdout, 'Result saved to out/result.txt (0 KB)\n');
      assert.equal(await readFile(path.join(home, 'out/result.txt'), 'utf8'), '42');
      const aliased = await pw(['eval-file', 'script.js', '--filename', 'alias.txt']);
      assert.equal(aliased.code, 0, aliased.stderr);
      assert.equal(aliased.stdout, 'Result saved to alias.txt (0 KB)\n');
      const before = fake.calls.length;
      const missing = await pw(['eval-file', 'nope.js']);
      assert.equal(missing.code, 1);
      assert.match(missing.stderr, /^eval-file: cannot read nope\.js: /);
      const directory = await pw(['eval-file', 'out']);
      assert.equal(directory.code, 1);
      assert.equal(directory.stderr, 'eval-file: cannot read out: not a file\n');
      assert.equal(
        fake.calls.slice(before).filter((call) => call.params?.objectGroup === 'slicc-eval').length,
        0
      );
      const none = await pw(['eval-file']);
      assert.equal(none.stderr, 'eval-file requires a file path\n');
      const both = await pw(['eval-file', 'script.js', '--output', 'a', '--filename', 'b']);
      assert.equal(
        both.stderr,
        'eval-file: --filename and --output are aliases \u2014 pass one, not both\n'
      );
      assert.equal(evaluated.length, 3);
    }
  );
});

test('eval-file refuses protected tabs before evaluating', async () => {
  const evaluated = [];
  await withFake(
    remoteEval((params) => {
      evaluated.push(params.expression);
      return { type: 'number', value: 1 };
    }),
    async (_fake, pw, home) => {
      await writeFile(path.join(home, 'script.js'), 'document.cookie');
      const refused = await pw(['eval-file', 'script.js', '--tab', 'SEVEN']);
      assert.equal(refused.code, 1);
      assert.match(refused.stderr, /^slicc: refused: /);
      assert.deepEqual(evaluated, []);
    },
    [{ targetId: 'SEVEN', url: 'https://seven.sliccy.ai/os/' }]
  );
});

function fetchBackend(outcome) {
  const seen = [];
  const intercept = (message, _send, answer) => {
    if (message.method !== 'Runtime.evaluate') return false;
    const expression = message.params.expression ?? '';
    if (expression.startsWith('new URL(')) {
      const input = JSON.parse(expression.slice(8, expression.indexOf(', document.baseURI')));
      answer({
        result: { type: 'string', value: new URL(input, 'https://example.test/app/').href },
      });
      return true;
    }
    if (expression.includes('await fetch(')) {
      seen.push(message.params);
      answer(outcome(message.params));
      return true;
    }
    return false;
  };
  return { seen, intercept };
}

test('fetch runs in the tab, lists ordinary Link entries and drops protected ones', async () => {
  const { seen, intercept } = fetchBackend(() => ({
    result: {
      type: 'object',
      value: {
        status: 200,
        url: 'https://example.test/app/data?id=QUERYSECRET',
        redirected: true,
        link: linkHeader,
      },
    },
  }));
  await withFake(intercept, async (_fake, pw) => {
    const result = await pw(['fetch', 'data?id=QUERYSECRET', '--method', 'HEAD']);
    assert.equal(result.code, 0, result.stderr);
    assert.equal(result.stderr, '');
    const payload = JSON.parse(result.stdout);
    assert.equal(payload.status, 200);
    assert.equal(payload.redirected, true);
    assert.equal(payload.url, 'https://example.test/app/data?id=\u2026');
    assert.deepEqual(
      payload.links.map((link) => [link.href, link.rel]),
      [
        ['https://example.test/next?token=\u2026', 'next'],
        ['https://example.test/skills/demo', 'https://www.sliccy.ai/rel/upskill'],
      ]
    );
    assert.deepEqual(payload.handoff, {
      verb: 'upskill',
      target: 'https://example.test/skills/demo',
      instruction: 'Install the demo',
    });
    assert.doesNotMatch(result.stdout, /SECRET|sliccy\.ai\/os|vault/);
    assert.equal(seen.length, 1);
    assert.equal(seen[0].awaitPromise, true);
    assert.ok(seen[0].expression.includes('"https://example.test/app/data?id=QUERYSECRET"'));
    assert.ok(seen[0].expression.includes('method: "HEAD"'));
    assert.ok(seen[0].expression.includes("credentials: 'include'"));
    assert.doesNotMatch(seen[0].expression, /\.text\(\)|arrayBuffer|\.json\(\)/);
  });
});

test('fetch refuses protected destinations before connecting and after resolving', async () => {
  const { seen, intercept } = fetchBackend(() => ({ result: { type: 'object', value: {} } }));
  await withFake(intercept, async (fake, pw) => {
    for (const url of [
      'https://seven.sliccy.ai/auth/callback?code=SECRET',
      'http://vault.sliccy.ai/',
      '//seven.sliccy.ai/x',
    ]) {
      const refused = await pw(['fetch', url]);
      assert.equal(refused.code, 1, url);
      assert.match(refused.stderr, /^slicc: refused: /, url);
      assert.doesNotMatch(refused.stderr + refused.stdout, /SECRET/);
    }
    for (const [args, message] of [
      [['fetch'], 'fetch requires a URL\n'],
      [['fetch', 'ftp://example.test/x'], 'fetch: only http and https URLs can be fetched\n'],
      [['fetch', 'javascript:alert(1)'], 'fetch: only http and https URLs can be fetched\n'],
      [
        ['fetch', 'https://example.test/', '--method', 'GET X'],
        'fetch: --method must be an HTTP method token\n',
      ],
    ]) {
      const bad = await pw(args);
      assert.equal(bad.code, 1, args.join(' '));
      assert.equal(bad.stderr, message, args.join(' '));
    }
    assert.equal(fake.calls.length, 0);
    assert.equal(seen.length, 0);
  });
});

test('fetch never sends a request whose tab-relative URL resolves to a protected origin', async () => {
  const seen = [];
  const intercept = (message, _send, answer) => {
    if (message.method !== 'Runtime.evaluate') return false;
    const expression = message.params.expression ?? '';
    if (expression.startsWith('new URL(')) {
      answer({ result: { type: 'string', value: 'https://seven.sliccy.ai/os/?code=SECRET' } });
      return true;
    }
    if (expression.includes('await fetch(')) seen.push(expression);
    return false;
  };
  await withFake(intercept, async (_fake, pw) => {
    const refused = await pw(['fetch', 'relative']);
    assert.equal(refused.code, 1);
    assert.match(refused.stderr, /^slicc: refused: /);
    assert.doesNotMatch(refused.stderr + refused.stdout, /SECRET/);
    assert.deepEqual(seen, []);
  });
});

test('a failed fetch prints JSON with the error and exits 1', async () => {
  const { intercept } = fetchBackend(() => ({
    result: { type: 'object', subtype: 'error' },
    exceptionDetails: {
      text: 'Uncaught',
      exception: { description: 'TypeError: Failed to fetch' },
    },
  }));
  await withFake(intercept, async (_fake, pw) => {
    const failed = await pw(['fetch', 'https://example.test/redirect']);
    assert.equal(failed.code, 1);
    assert.deepEqual(JSON.parse(failed.stdout), {
      error: 'TypeError: Failed to fetch',
      handoff: null,
      links: [],
      url: 'https://example.test/redirect',
    });
  });
});

test('generate-locator prints the locator from Playwright generator for a snapshot ref', async () => {
  await withFake(locatorBackend, async (fake, pw) => {
    const missing = await pw(['generate-locator']);
    assert.equal(missing.code, 1);
    assert.equal(missing.stderr, "error: 'target' argument: expected string, received undefined\n");
    const selector = await pw(['generate-locator', 'button']);
    assert.equal(selector.code, 1);
    assert.match(selector.stderr, /not an element ref/);
    const early = await pw(['generate-locator', 'e2']);
    assert.equal(early.code, 1);
    assert.equal(early.stderr, 'No snapshot available. Run "snapshot" first.\n');
    const snapshot = await pw(['snapshot']);
    assert.equal(snapshot.code, 0, snapshot.stderr);
    for (let round = 0; round < 2; round += 1) {
      const result = await pw(['generate-locator', 'e2']);
      assert.equal(result.code, 0, result.stderr);
      assert.equal(result.stdout, "### Result\ngetByRole('button', { name: 'Go' })\n");
    }
    const resolved = sent(fake, 'DOM.resolveNode').filter(
      (call) => call.params.executionContextId !== undefined
    );
    assert.equal(resolved.length, 2);
    assert.equal(resolved[0].params.backendNodeId, 2);
    const worlds = sent(fake, 'Page.createIsolatedWorld');
    assert.equal(worlds.length >= 2, true);
    const generated = sent(fake, 'Runtime.callFunctionOn').filter((call) =>
      call.params.functionDeclaration.includes('generateSelectorSimple')
    );
    assert.equal(generated.length, 2);
    assert.equal(generated[0].params.returnByValue, true);
    assert.deepEqual(generated[0].params.arguments, [{ value: null }]);
    assert.ok(generated[0].params.functionDeclaration.includes('sliccInjectedScriptClass'));
    assert.equal(sent(fake, 'Network.emulateNetworkConditions').length, 0);
    const unknown = await pw(['generate-locator', 'e9']);
    assert.equal(unknown.code, 1);
    assert.match(unknown.stderr, /^Unknown ref "e9"/);
  });
});

test('generate-locator and highlight refuse protected tabs', async () => {
  await withFake(
    locatorBackend,
    async (fake, pw) => {
      for (const args of [
        ['generate-locator', 'e2', '--tab', 'SEVEN'],
        ['highlight', 'e2', '--tab', 'SEVEN'],
        ['highlight', '--hide', '--tab', 'SEVEN'],
      ]) {
        const refused = await pw(args);
        assert.equal(refused.code, 1, args.join(' '));
        assert.match(refused.stderr, /^slicc: refused: /, args.join(' '));
      }
      assert.deepEqual(touched(fake, 'SEVEN'), []);
    },
    [{ targetId: 'SEVEN', url: 'https://seven.sliccy.ai/os/' }]
  );
});

test('highlight shows, hides and clears overlays with upstream output', async () => {
  await withFake(
    (message, send, answer) => {
      if (
        message.method === 'Runtime.evaluate' &&
        message.params.expression?.includes('x-slicc-highlight')
      ) {
        answer({ result: { type: 'undefined' } });
        return true;
      }
      return locatorBackend(message, send, answer);
    },
    async (fake, pw) => {
      const bare = await pw(['highlight']);
      assert.equal(bare.code, 1);
      assert.equal(
        bare.stdout,
        '### Error\nInvalid arguments for tool "browser_highlight":\n\u2716 Invalid input: expected string, received undefined\n  \u2192 at target\n'
      );
      assert.equal(fake.calls.length, 0);
      const snapshot = await pw(['snapshot']);
      assert.equal(snapshot.code, 0, snapshot.stderr);
      const shown = await pw(['highlight', 'e2', '--style', 'outline: 2px dashed red']);
      assert.equal(shown.code, 0, shown.stderr);
      assert.equal(shown.stdout, "### Result\nHighlighted locator('aria-ref=e2')\n");
      const hidden = await pw(['highlight', 'e2', '--hide']);
      assert.equal(hidden.code, 0, hidden.stderr);
      assert.equal(hidden.stdout, "### Result\nHid highlight for locator('aria-ref=e2')\n");
      const calls = sent(fake, 'Runtime.callFunctionOn').filter((call) =>
        call.params.functionDeclaration.includes('x-slicc-highlight')
      );
      assert.deepEqual(
        calls.map((call) => call.params.arguments),
        [[{ value: 'outline: 2px dashed red' }, { value: '2' }], [{ value: '2' }]]
      );
      const cleared = await pw(['highlight', '--hide']);
      assert.equal(cleared.code, 0, cleared.stderr);
      assert.equal(cleared.stdout, '### Result\nHid page highlight\n');
      const wiped = sent(fake, 'Runtime.evaluate').filter((call) =>
        call.params.expression?.includes('x-slicc-highlight')
      );
      assert.equal(wiped.length, 1);
      assert.equal(typeof wiped[0].params.uniqueContextId, 'string');
      const again = await pw(['highlight', 'e2']);
      assert.equal(again.code, 0, again.stderr);
      assert.ok(pageSession(fake));
    }
  );
});

test('kernel page tools behave like the native build', async () => {
  const evaluated = [];
  const { intercept: fetching } = fetchBackend(() => ({
    result: {
      type: 'object',
      value: { status: 204, url: 'https://example.test/app/ping', redirected: false, link: null },
    },
  }));
  const intercept = (message, send, answer) =>
    fetching(message, send, answer) ||
    locatorBackend(message, send, answer) ||
    remoteEval((params) => {
      evaluated.push(params.expression);
      return { type: 'string', value: 'from-file' };
    })(message, send, answer);
  await withKernel(intercept, async (fake, pw, runner, cwd) => {
    const offline = await pw(['network-state-set', 'offline']);
    assert.equal(offline.code, 0, offline.stderr);
    assert.match(offline.stdout, /^### Result\nNetwork is now offline\n/);
    await runner.write(cwd, 'script.js', '"from-file"');
    const file = await pw(['eval-file', 'script.js']);
    assert.equal(file.code, 0, file.stderr);
    assert.equal(file.stdout, 'from-file\n');
    assert.deepEqual(evaluated, ['"from-file"']);
    assert.deepEqual(sent(fake, 'Network.emulateNetworkConditions')[0]?.params, conditions(true));
    const online = await pw(['network-state-set', 'online']);
    assert.equal(online.code, 0, online.stderr);
    const fetched = await pw(['fetch', 'ping']);
    assert.equal(fetched.code, 0, fetched.stderr);
    assert.deepEqual(JSON.parse(fetched.stdout), {
      handoff: null,
      links: [],
      redirected: false,
      status: 204,
      url: 'https://example.test/app/ping',
    });
    const refused = await pw(['fetch', 'https://vault.sliccy.ai/']);
    assert.equal(refused.code, 1);
    assert.match(refused.stderr, /^slicc: refused: /);
    const snapshot = await pw(['snapshot']);
    assert.equal(snapshot.code, 0, snapshot.stderr);
    const located = await pw(['generate-locator', 'e2']);
    assert.equal(located.code, 0, located.stderr);
    assert.equal(located.stdout, "### Result\ngetByRole('button', { name: 'Go' })\n");
    const shown = await pw(['highlight', 'e2']);
    assert.equal(shown.code, 0, shown.stderr);
    assert.equal(shown.stdout, "### Result\nHighlighted locator('aria-ref=e2')\n");
    assert.equal(sent(fake, 'Network.emulateNetworkConditions').length, 1);
  });
});
