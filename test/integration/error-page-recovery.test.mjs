import assert from 'node:assert/strict';
import { execFileSync, spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';
import { HARNESS_PORTS } from './ports.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const cli = path.join(
  root,
  'target/debug',
  process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'
);
const ERROR_PAGE = 'chrome-error://chromewebdata/';
const SEVEN = "slicc: refused: target is on seven's origin";
const INHERITED =
  'slicc: refused: a browser-side navigation to about: gives the page the origin of whatever drives the browser, so the guard doesn’t send it; open a new tab on about:blank instead';

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
    const child = spawn(cli, args, { cwd: home, env, stdio: ['ignore', 'pipe', 'pipe'] });
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

test('a fresh playwright-cli can recover an opened about:blank tab whose blocked vault error page an upstream guard masked, but not read it', {
  timeout: 60_000,
}, async () => {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-error-page-'));
  const fake = await fakeCdp({ refuse: ['Page.navigate'], refusalMessage: INHERITED });
  const pw = (args) => run(['--cdp', fake.url, ...args], home);
  try {
    const opened = await pw(['open', 'about:blank']);
    assert.equal(opened.code, 0, opened.stderr);
    const [tab] = fake.targets;
    tab.url = ERROR_PAGE;
    tab.frame = { id: tab.targetId, loaderId: 'LERR', url: ERROR_PAGE, securityOrigin: '://' };

    const blank = await pw(['goto', 'about:blank']);
    assert.notEqual(blank.code, 0);
    assert.doesNotMatch(blank.stderr, new RegExp(SEVEN), blank.stderr);
    assert.match(blank.stderr, /a browser-side navigation to about:/, blank.stderr);
    const spaced = await pw(['goto', ' about:blank']);
    assert.notEqual(spaced.code, 0);
    assert.doesNotMatch(spaced.stderr, new RegExp(SEVEN), spaced.stderr);
    const from = fake.calls.length;
    const content = await pw(['eval', 'document.documentElement.innerHTML']);
    assert.notEqual(content.code, 0);
    assert.match(content.stderr, new RegExp(SEVEN), content.stderr);
    assert.ok(
      fake.calls
        .slice(from)
        .every(
          (call) =>
            call.method !== 'Runtime.evaluate' ||
            /^(document\.readyState|0)$/.test(call.params?.expression)
        ),
      'the masked error page ran a caller expression'
    );
    const reload = await pw(['reload']);
    assert.doesNotMatch(reload.stderr, new RegExp(SEVEN), reload.stderr);

    tab.url = 'http://vault.sliccy.ai/vault';
    const vault = await pw(['eval', 'self.origin']);
    assert.notEqual(vault.code, 0);
    assert.match(vault.stderr, new RegExp(SEVEN), vault.stderr);
    assert.doesNotMatch(vault.stdout, /fake-eval/);
  } finally {
    await fake.close();
    await rm(home, { recursive: true, force: true });
  }
});

test('a caller-observed recovery from an error page stays drivable when the guard cannot watch Page events', {
  timeout: 60_000,
}, async () => {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-error-page-'));
  const guarded = (calls, sessionId) =>
    calls.some(
      (call) =>
        call.sessionId === sessionId &&
        call.method === 'Fetch.enable' &&
        call.params?.patterns?.some((item) => item.resourceType === 'Document')
    );
  const fake = await fakeCdp({
    refuseWhen: (message, calls) =>
      message.method === 'Page.enable' && guarded(calls, message.sessionId),
  });
  const pw = (args) => run(['--cdp', fake.url, ...args], home);
  try {
    const opened = await pw(['open', 'about:blank']);
    assert.equal(opened.code, 0, opened.stderr);
    const [tab] = fake.targets;
    const failed = 'http://ordinary.invalid/';
    tab.url = failed;
    tab.frame = {
      id: tab.targetId,
      loaderId: 'LERR',
      url: ERROR_PAGE,
      securityOrigin: '://',
      unreachableUrl: failed,
    };
    const from = fake.calls.length;
    const recovered = await pw(['goto', 'http://ordinary.example/']);
    assert.equal(recovered.code, 0, recovered.stderr);
    assert.doesNotMatch(recovered.stderr, new RegExp(SEVEN), recovered.stderr);
    assert.ok(
      fake.calls
        .slice(from)
        .some((call) => call.method === 'Page.enable' && guarded(fake.calls, call.sessionId)),
      'the guard asked for Page events and was refused'
    );
    const origin = await pw(['eval', 'self.origin']);
    assert.equal(origin.code, 0, origin.stderr);
    assert.match(origin.stdout, /fake-eval/);
  } finally {
    await fake.close();
    await rm(home, { recursive: true, force: true });
  }
});

for (const attachEvents of [false, true]) {
  test(`a fresh playwright-cli navigates away from an unreadable error tab regardless of its history${attachEvents ? ', with attach events' : ''}`, {
    timeout: 60_000,
  }, async () => {
    const home = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-error-page-'));
    const fake = await fakeCdp({ attachEvents });
    const pw = (args) => run(['--cdp', fake.url, ...args], home);
    const start = 'http://ordinary.example/doc';
    const failed = 'https://seven.sliccy.ai/auth/callback?code=MARK#access_token=MARK';
    const errored = (tab, entries) => {
      tab.url = failed;
      tab.frame = {
        id: tab.targetId,
        loaderId: 'LERR',
        url: ERROR_PAGE,
        securityOrigin: '://',
        unreachableUrl: failed,
      };
      tab.history = { currentIndex: entries.length - 1, entries };
    };
    const entry = (id, url, transitionType = 'typed') => ({
      id,
      url,
      userTypedURL: url,
      title: url,
      transitionType,
    });
    try {
      const opened = await pw(['open', start]);
      assert.equal(opened.code, 0, opened.stderr);
      const [tab] = fake.targets;
      errored(tab, [entry(1, start), entry(2, failed)]);
      const destination = 'http://ordinary.example/recovered';
      const recovered = await pw(['goto', destination, '--tab', tab.targetId]);
      assert.equal(recovered.code, 0, recovered.stderr);
      assert.doesNotMatch(`${recovered.stdout}${recovered.stderr}`, /MARK/);
      assert.equal(tab.url, destination);
      const body = await pw(['eval', 'document.body.textContent', '--tab', tab.targetId]);
      assert.equal(body.code, 0, body.stderr);

      for (const entries of [
        [entry(1, failed)],
        [entry(1, 'https://seven.sliccy.ai/'), entry(2, start), entry(3, failed)],
        [entry(1, start), entry(2, failed, 'reload')],
      ]) {
        errored(tab, entries);
        const read = await pw(['eval', 'document.body.textContent', '--tab', tab.targetId]);
        assert.notEqual(read.code, 0);
        assert.match(read.stderr, new RegExp(SEVEN), read.stderr);
        assert.doesNotMatch(`${read.stdout}${read.stderr}`, /MARK/);
        const recovered = await pw(['goto', destination, '--tab', tab.targetId]);
        assert.equal(recovered.code, 0, recovered.stderr);
        assert.doesNotMatch(`${recovered.stdout}${recovered.stderr}`, /MARK/);
        assert.equal(tab.url, destination);
      }
    } finally {
      await fake.close();
      await rm(home, { recursive: true, force: true });
    }
  });
}

const seeded = (targetId, failed, entries) => ({
  targetId,
  url: failed,
  frame: {
    id: targetId,
    loaderId: 'LERR',
    url: ERROR_PAGE,
    securityOrigin: '://',
    unreachableUrl: failed,
  },
  history: { currentIndex: entries.length - 1, entries },
});

const visit = (id, url) => ({ id, url, userTypedURL: url, title: url, transitionType: 'typed' });

async function bridged(options, body) {
  execFileSync(
    'cargo',
    ['build', '--locked', '-p', 'slicc-cdp-client', '--example', 'guard_bridge'],
    { stdio: 'inherit' }
  );
  const backend = await fakeCdp(options);
  let client;
  try {
    client = await guarded(backend.url, 'https://private.example.invalid', {
      variant: 'prompt',
      detachDelay: 0,
    });
    await body(backend, client);
  } finally {
    await client?.close();
    await backend.close();
  }
}

test('discovery of an already open protected error tab leaves it to the recovery probe', {
  timeout: 120_000,
}, async () => {
  const failed = 'https://seven.sliccy.ai/auth/callback?code=MARK';
  const start = 'http://ordinary.example/doc';
  await bridged({ discoverEvents: true }, async (backend, client) => {
    backend.targets.push(seeded('ERRTAB', failed, [visit(1, start), visit(2, failed)]));
    const { targetId } = await client.send('Target.createTarget', { url: 'about:blank' });
    assert.notEqual(targetId, 'ERRTAB');
    const discovered = client.events.filter(
      (event) => event.params?.targetInfo?.targetId === 'ERRTAB'
    );
    assert.ok(discovered.length > 0, 'discovery reported the open error tab');
    assert.ok(discovered.every((event) => !JSON.stringify(event).includes('MARK')));
    const { sessionId } = await client.send('Target.attachToTarget', {
      targetId: 'ERRTAB',
      flatten: true,
    });
    await client.send('Page.navigate', { url: 'http://ordinary.example/next' }, sessionId);
    assert.equal(
      backend.targets.find((item) => item.targetId === 'ERRTAB').url,
      'http://ordinary.example/next'
    );
  });
});

test('an unobserved protected commit after a recoverable error page is caught before recovery commands run', {
  timeout: 120_000,
}, async () => {
  const failed = 'https://seven.sliccy.ai/auth/callback?code=MARK';
  const start = 'http://ordinary.example/doc';
  const guardSession = (calls, sessionId) =>
    calls.some(
      (call) =>
        call.sessionId === sessionId &&
        call.method === 'Fetch.enable' &&
        call.params?.patterns?.some((item) => item.resourceType === 'Document')
    );
  await bridged(
    {
      refuseWhen: (message, calls) =>
        message.method === 'Page.enable' && guardSession(calls, message.sessionId),
    },
    async (backend, client) => {
      const tab = seeded('ERRTAB', failed, [visit(1, start), visit(2, failed)]);
      backend.targets.push(tab);
      const { sessionId } = await client.send('Target.attachToTarget', {
        targetId: 'ERRTAB',
        flatten: true,
      });
      tab.frame = {
        id: 'ERRTAB',
        loaderId: 'LREAL',
        url: 'https://seven.sliccy.ai/auth/callback',
        securityOrigin: 'https://seven.sliccy.ai',
      };
      const from = backend.calls.length;
      for (const [method, params] of [
        ['Runtime.evaluate', { expression: 'document.readyState', returnByValue: true }],
        ['Page.getNavigationHistory', {}],
      ]) {
        await assert.rejects(client.send(method, params, sessionId), /slicc: refused:/, method);
      }
      assert.ok(
        backend.calls
          .slice(from)
          .every(
            (call) =>
              call.sessionId !== sessionId ||
              !['Runtime.evaluate', 'Page.getNavigationHistory'].includes(call.method)
          ),
        'a recovery command reached the protected document'
      );
    }
  );
});

test('a fresh playwright-cli recovers a blocked tab whose target info an upstream guard reports as the protected origin, and still refuses a real protected page', {
  timeout: 60_000,
}, async () => {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-error-page-'));
  const fake = await fakeCdp({ attachEvents: true });
  const pw = (args) => run(['--cdp', fake.url, ...args], home);
  const start = 'http://ordinary.example/page';
  const origin = 'http://vault.sliccy.ai';
  const failed = `${origin}/landing?n=refresh&k=MARK`;
  try {
    const opened = await pw(['open', start]);
    assert.equal(opened.code, 0, opened.stderr);
    const [tab] = fake.targets;
    for (const current of [failed, origin]) {
      Object.assign(tab, seeded(tab.targetId, failed, [visit(1, start), visit(2, current)]));
      tab.url = origin;
      const destination = `http://ordinary.example/after-${current === origin ? 'origin' : 'url'}`;
      const recovered = await pw(['goto', destination, '--tab', tab.targetId]);
      assert.equal(recovered.code, 0, recovered.stderr);
      assert.doesNotMatch(`${recovered.stdout}${recovered.stderr}`, /MARK/);
      assert.equal(tab.url, destination);
    }
    tab.url = origin;
    tab.frame = {
      id: tab.targetId,
      loaderId: 'LREAL',
      url: `${origin}/landing`,
      securityOrigin: origin,
    };
    tab.history = { currentIndex: 1, entries: [visit(1, start), visit(2, `${origin}/landing`)] };
    const refused = await pw(['eval', 'document.body.textContent', '--tab', tab.targetId]);
    assert.notEqual(refused.code, 0);
    assert.match(refused.stderr, new RegExp(SEVEN), refused.stderr);
    assert.equal(tab.url, origin);
    const destination = 'http://ordinary.example/next';
    const recovered = await pw(['goto', destination, '--tab', tab.targetId]);
    assert.notEqual(recovered.code, 0);
    assert.match(recovered.stderr, new RegExp(SEVEN), recovered.stderr);
    assert.equal(tab.url, origin);
  } finally {
    await fake.close();
    await rm(home, { recursive: true, force: true });
  }
});
